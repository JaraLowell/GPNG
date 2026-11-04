USER — Graphic Packet user directory

7Plus parts are saved here when the 7+ switch is ON.
7Plus is the amateur-radio text encoding (DG1BBQ), not 7-Zip.
It rewrites a binary file as ASCII lines so those bytes can travel
inside an AX.25 packet. A part looks like:

    go_7+.
    ...
    name.p01
    ...
    stop_7+.

//Read name.p01
    sends that file from this folder to the connected station.

//Read
    lists this folder.

//7+ ON
//7+ OFF
    same switch as the 7+ button on the session bar.

The encoding is the C program at https://github.com/hb9xar/7plus
(7PLUS 2.2 by Axel Bauda, DG1BBQ). This folder only stores the
text parts. Decoding them still needs that program.
